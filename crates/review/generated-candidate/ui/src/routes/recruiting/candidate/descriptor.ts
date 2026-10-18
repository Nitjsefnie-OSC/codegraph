import type { EntityDescriptor } from '@crewbase/entities';

export const CandidateDescriptor: EntityDescriptor = {
  name: 'Candidate',
  domain: 'recruiting',
  pathSegment: 'candidate',
  operations: ['create', 'read', 'update', 'list'],

  fields: [

    {
      name: 'birth_date',
      label: 'Birth Date',
      type: 'date',
      tsType: 'string',





      validation: {






        format: 'date',

      },





    },

    {
      name: 'family_name',
      label: 'Family Name',
      type: 'text',
      tsType: 'string',

      required: true,




      description: 'Last name',






    },

    {
      name: 'given_name',
      label: 'Given Name',
      type: 'text',
      tsType: 'string',

      required: true,




      description: 'First name',






    },

    {
      name: 'application_process_history',
      label: 'Application Process History',
      type: 'text',
      tsType: 'CandidateProcessHistoryResponse',




      description: 'Application process history (array-type schema)',






    },

    {
      name: 'candidate_id',
      label: 'Candidate Id',
      type: 'text',
      tsType: 'string',

      required: true,




      description: 'Unique candidate identifier',



      list: { visible: true, sortable: true },




    },

    {
      name: 'compensation_expectation',
      label: 'Compensation Expectation',
      type: 'number',
      tsType: 'string',









    },

    {
      name: 'compensation_expectation_currency',
      label: 'Compensation Expectation Currency',
      type: 'select',
      tsType: 'string',








      options: {
        source: 'inline',


        values: [

          { value: 'USD', label: 'USD' },

          { value: 'EUR', label: 'EUR' },

          { value: 'GBP', label: 'GBP' },

          { value: 'JPY', label: 'JPY' },

          { value: 'AUD', label: 'AUD' },

        ],

      },


    },

    {
      name: 'distribution_guidelines',
      label: 'Distribution Guidelines',
      type: 'text',
      tsType: 'CandidateDistributionGuidelinesResponse',




      description: 'Distribution guidelines (inline def with allOf)',






    },

    {
      name: 'external_identifier',
      label: 'External Identifier',
      type: 'text',
      tsType: 'string',




      description: 'External identifier (structured wrapper / JSONB)',






    },

    {
      name: 'gender',
      label: 'Gender',
      type: 'select',
      tsType: 'string',




      description: 'Gender codelist reference',





      options: {
        source: 'inline',


        values: [

          { value: 'Male', label: 'Male' },

          { value: 'Female', label: 'Female' },

          { value: 'NotSpecified', label: 'NotSpecified' },

          { value: 'Other', label: 'Other' },

        ],

      },


    },

    {
      name: 'person_name',
      label: 'Person Name',
      type: 'text',
      tsType: 'CandidateNameResponse',




      description: 'Candidate name (value object)',






    },

    {
      name: 'position_schedule_type_codes',
      label: 'Position Schedule Type Codes',
      type: 'select',
      tsType: 'string',




      description: 'Schedule type preferences (array of codelist)',





      options: {
        source: 'inline',


        values: [

          { value: 'FullTime', label: 'FullTime' },

          { value: 'PartTime', label: 'PartTime' },

          { value: 'FlexTime', label: 'FlexTime' },

          { value: 'SharedTime', label: 'SharedTime' },

        ],

      },


    },

    {
      name: 'position_titles',
      label: 'Position Titles',
      type: 'array',
      tsType: 'Array<string>',




      description: 'Preferred position titles (array_wrapper)',






    },

    {
      name: 'qualifications',
      label: 'Qualifications',
      type: 'text',
      tsType: 'CandidateQualificationResponse',




      description: 'Array of value-object qualifications',






    },

    {
      name: 'referred_by_application_id',
      label: 'Referred By Application Id',
      type: 'text',
      tsType: 'string',




      description: 'Source application (entity reference)',




      ref: { entity: 'ApplicationType', domain: 'recruiting', displayField: 'application_id' },



    },

    {
      name: 'status',
      label: 'Status',
      type: 'select',
      tsType: 'string',




      description: 'Current candidate status',



      list: { visible: true, badge: true },



      options: {
        source: 'inline',


        values: [

          { value: 'active', label: 'active' },

          { value: 'inactive', label: 'inactive' },

          { value: 'withdrawn', label: 'withdrawn' },

        ],

      },


    },

    {
      name: 'uri',
      label: 'Uri',
      type: 'text',
      tsType: 'string',




      description: 'Public profile URL',


      validation: {






        format: 'uri',

      },


      list: { visible: true },




    },

  ],


  children: [

    {
      name: 'distribution_guidelines',
      label: 'DistributionGuidelines',
      entity: 'DistributionGuidelines',
      relationship: 'one-to-many',
      inline: false,
      displayField: 'start_date',
    },

    {
      name: 'qualification',
      label: 'Qualification',
      entity: 'Qualification',
      relationship: 'one-to-many',
      inline: false,
      displayField: 'qualification_name',
    },

  ],


  workflow: {
    field: 'candidate_status_code',
    transitions: [

      { from: 'interviewing', to: 'offer', label: 'Offer', confirm: true },

      { from: 'interviewing', to: 'rejected', label: 'Rejected', confirm: true },

      { from: 'interviewing', to: 'withdrawn', label: 'Withdrawn', confirm: true },

      { from: 'new', to: 'rejected', label: 'Rejected', confirm: true },

      { from: 'new', to: 'screening', label: 'Screening', confirm: true },

      { from: 'new', to: 'withdrawn', label: 'Withdrawn', confirm: true },

      { from: 'offer', to: 'hired', label: 'Hired', confirm: true },

      { from: 'offer', to: 'rejected', label: 'Rejected', confirm: true },

      { from: 'offer', to: 'withdrawn', label: 'Withdrawn', confirm: true },

      { from: 'screening', to: 'interviewing', label: 'Interviewing', confirm: true },

      { from: 'screening', to: 'rejected', label: 'Rejected', confirm: true },

      { from: 'screening', to: 'withdrawn', label: 'Withdrawn', confirm: true },

    ],
  },


  wizard: {
    enabled: true,
    steps: [

      { key: 'basics', label: 'Candidate Type Details', source: 'self', groups: ['default'] },

      { key: 'distribution_guidelines', label: 'Distribution Guidelines', source: 'child', child: 'DistributionGuidelines', cardinality: 'many' },

      { key: 'qualification', label: 'Qualification', source: 'child', child: 'Qualification', cardinality: 'many' },

      { key: 'summary', label: 'Review & Submit', source: 'summary' },

    ],
  },

};
